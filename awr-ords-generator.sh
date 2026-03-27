#!/usr/bin/bash -x
###############################
#
# Author: Julian Frey
# Email: julian.frey@cite.ch
#
# This script creates AWR reports via ORDS REST API for a specific PDB. You can either generate a report for a single snapshot range or for all snapshot intervals within a specified range.
#
# Usage:
# getAWR.sh -p PDB_NAME -u USER_NAME [options]
# Example:
# getAWR.sh -p PDB1 -u juf -o dghost1 -n 8080 -i 1166464959 -c 1 -l 100 -t 110 -s true -m SINGLE -d /path/to/output
#
# Options:
# -p     | --pdb_name              PDB to connect
# -u     | --username              User to connect
# -o     | --ords_host             ORDS Host (default: dghost1)
# -n     | --ords_port             ORDS Port (default: 8080)
# -i     | --db_id                 Database ID
# -c     | --db_con_id             Database Container ID
# -l     | --low_snp_id            Low Snapshot ID (optional)
# -t     | --top_snap_id           High Snapshot ID (optional) (if only -l is provided all spanshots will be created up to the highest)
# -s     | --single_pdb            Single PDB mode for the ORDS configuration (default: false)
# -m     | --mode                  Mode (INTERVALL/SINGLE) (default: INTERVALL)
# -d     | --directory             Output Directory (default: current directory)
# -z     | --zip_file              Zipfile for compressed outputs
# -zd    | --delete_files          Delete individual AWR report files after creating zipfile (true/false default: false)
# -zm    | --mail_recipients       Mail recipients for sending the zip file (comma separated) (default: false = no mail  sent)
# -oci   | --oci_url               OCI URL for AWR Reports form autonomous DBs
# -j     | --jas_min               Send folder to jas-min for analysis
# -jp    | --jas_min_path          Path to the jas-min executable
# -jt    | --jas_min_tuning        jas-min tuning parameters
# -h     | --help                  Show this help message
#
###############################
usage()
{
cat << EOF
usage: bash ./$SCRIPT_NAME -p PDB_NAME -u USER_NAME 
-p     | --pdb_name              PDB to connect
-u     | --username              User to connect
-o     | --ords_host             ORDS Host (default: dghost1)
-n     | --ords_port             ORDS Port (default: 8080)
-i     | --db_id                 Database ID
-c     | --db_con_id             Database Container ID
-l     | --low_snp_id            Low Snapshot ID (optional)
-t     | --top_snap_id           High Snapshot ID (optional) (if only -l is provided all spanshots will be created up to the highest)
-s     | --single_pdb            Single PDB mode (default: false)
-m     | --mode                  Mode (INTERVALL/SINGLE) (default: INTERVALL)
-d     | --directory             Output Directory (default: current directory)
-z     | --zip_file              Zipfile for compressed outputs
-zd    | --delete_files          Delete individual AWR report files after creating zipfile (true/false default: false)
-zm    | --mail_recipients       Mail recipients for sending the zip file (comma separated) (default: false = no mail  sent)
-oci   | --oci_url               OCI URL for AWR Reports form autonomous DBs
-j     | --jas_min               Send folder to jas-min for analysis
-jp    | --jas_min_path          Path to the jas-min executable
-jt    | --jas_min_tuning        jas-min tuning parameters
-h     | --help                  Show this help message
EOF
}
while [ "$1" != "" ]; do
    case $1 in
        -p | --pdb_name )
            shift
            DB_PDB=$1
        ;;
        -u | --user_name )
            shift
            DB_USER=$1   
        ;;
        -o | --ords_host )
            shift
            ORDS_HOST=$1
        ;;
        -n | --ords_port )
            shift
            ORDS_PORT=$1
        ;;
        -i | --db_id )
            shift
            DB_ID=$1
        ;;
        -c | --db_con_id )
            shift
            DB_CON_ID=$1
        ;;
        -l | --low_snp_id )
            shift
            if [ -z "$1" ]; then
              echo "Low Snapshot ID cannot be empty" >&2
              exit 1
            fi
            AWR_LOW=$1
        ;;
        -t | --top_snp_id )
            shift
            if [ -z "$1" ]; then
              echo "High Snapshot ID cannot be empty" >&2
              exit 1
            fi
            AWR_HIGH=$1
        ;;
        -s | --single_pdb )
            shift
            SINGLE_PDB=$1
        ;;
        -m | --mode )
            shift
            if [ "$1" != "INTERVALL" ] && [ "$1" != "SINGLE" ]; then
              echo "Invalid mode: $1. Allowed values are INTERVALL or SINGLE" >&2
              exit 1
            fi
            MODE=$1
        ;;
        -d | --directory )
            shift
            OUTPUTDIR=$1
        ;;
        -z | --zip_file )
            shift
            if [[ -z $1 || !($1 =~ ^(true|false)$) ]]; then
              echo "Zip file name cannot be empty and must be true or false when specified" >&2
              exit 1
            fi
            ZIP=true
            ZIP_FILE=$1
        ;;
        -zd | --delete_files )
            shift
            DELETE_FILES=true
        ;;
        -zm | --mail_recipients )
            shift
            if [ -z "$1" ]; then
              echo "Mail recipients cannot be empty" >&2
              exit 1
            fi
            MAIL_RECIPIENTS=$1
        ;;
        -oci | --oci_url )
            shift
            if [ -z "$1" ]; then
              echo "OCI URL cannot be empty" >&2
              exit 1
            fi
            OCI_URL=$1
        ;;
        -j | --jas_min )
            shift
            if [[ -z $1 || !($1 =~ ^(true|false)$) ]]; then
              echo "Jas-min cannot be empty and must be true or false when specified" >&2
              exit 1
            fi
            SEND_TO_JASMIN=true
        ;;
        -jp | --jas_min_path )
            shift
            if [ ! -x "$1/jas-min" ]; then
              echo "Jas-min is not executable: $1/jas-min" >&2
              exit 1
            fi
            JASMIN_PATH=$1
        ;;
        -jt | --jas_min_tuning )
            shift
            JASMIN_TUNING=$1
        ;;
        -h | --help )    usage
            exit
        ;;
        * )              usage
            exit 1
    esac
    shift
done

################################
#
# Variable Definition
#
################################

ORDS_HOST="${ORDS_HOST:-localhost}"
ORDS_PORT="${ORDS_PORT:-8080}"
DB_USER="${DB_USER:-admin}"
DB_PDB="/${DB_PDB:-PDB1}/"
DB_ID="${DB_ID:-1111111111}"
DB_CON_ID="${DB_CON_ID:-1}"
MODE="${MODE:-INTERVALL}"
SINGLE_PDB="${SINGLE_PDB:-false}"
OUTPUT_DIR="${OUTPUTDIR:-.}"
MAIL_RECIPIENTS="${MAIL_RECIPIENTS:-false}"
SEND_TO_JASMIN="${SEND_TO_JASMIN:-false}"
JASMIN_PATH="${JASMIN_PATH:-./jas-min/target/release}"
JASMIN_TUNING="${JASMIN_TUNING:-}"

if [[ $ZIP_FILE ]]; then
  TMPFILE=$(mktemp -d $OUTPUT_DIR)
fi

DELETE_FILES="${DELETE_FILES:-false}"
files_to_zip=()

if [ $SINGLE_PDB == "true" ]; then
  DB_PDB="/"
fi
if [[ $OCI_URL ]]; then
  ORDS_URL=$OCI_URL
else
  PROTOCOL="${PROTOCOL:-http}"
  ORDS_URL=${PROTOCOL}://${ORDS_HOST}:${ORDS_PORT}/ords${DB_PDB}${DB_USER}/_sdw/_services/dba/awr
fi
echo -n Password: 
read -s password
echo
echo $password
BASE64_AUTH=$(echo -n "${DB_USER}:${password}" | base64)

################################
#
# Validate Parameters
#
################################

if [[ ! -w $OUTPUT_DIR ]]; then
  echo "Output directory is not writable: $OUTPUT_DIR" >&2
  echo "trying to create it..."
  mkdir -p $OUTPUT_DIR
  if [ $? -ne 0 ]; then
      echo "Failed to create output directory: $OUTPUT_DIR" >&2
      exit 1
  fi
fi

if [[ $OCI_URL && ( -z $AWR_LOW || -z $AWR_HIGH ) ]]; then
  echo "For OCI URL both low and high snapshot IDs must be provided" >&2
  exit 1
fi

if [[ -z $OCI_URL ]]; then
  re='^[0-9]+$'
  AWR_LOW="${AWR_LOW:-` curl --request GET   --url $ORDS_URL/snapshots/${DB_ID}/${DB_CON_ID}/?q=%7B%22%24orderby%22%3A%7B%22snap_id%22%3A%22ASC%22%7D%7D  --header  "Authorization: Basic ${BASE64_AUTH}"  -s |jq '.items[0].snap_id' `}"
  AWR_HIGH="${AWR_HIGH:-` curl --request GET   --url $ORDS_URL/snapshots/${DB_ID}/${DB_CON_ID}/?q=%7B%22%24orderby%22%3A%7B%22snap_id%22%3A%22DESC%22%7D%7D --header "Authorization: Basic ${BASE64_AUTH}" -s |jq '.items[0].snap_id' `}"
  if ! [[ $AWR_LOW =~ $re ]] ; then
      echo "Error: Low Snapshot ID is not a number" >&2; exit 1
  fi
  if ! [[ $AWR_HIGH =~ $re ]] ; then
      echo "Error: High Snapshot ID is not a number" >&2; exit 1
  fi

fi

################################
#
# Function Definition
#
################################
getAwrReport(){
    low=$1
    high=$2
    curl --request GET --url ${ORDS_URL}/report/${DB_ID}/${DB_CON_ID}/${low}/${high} --header "Authorization: Basic ${BASE64_AUTH}"  -s > $OUTPUT_DIR/AWR_Report_${low}_${high}.html
    if [ $? -ne 0 ]; then
        echo "Failed to get AWR report for snapshot range $low to $high" >&2
    else
        echo "AWR report for snapshot range $low to $high saved to $OUTPUT_DIR/AWR_Report_${low}_${high}.html"
    fi
}

createZip(){
  zip -j "$OUTPUT_DIR/$ZIP_FILE" "${files_to_zip[@]}"

}
send_mail()
{
    SUBJECT="ZIP File of AWR Reports from $AWR_LOW to $AWR_HIGH "
    TO=$MAIL_RECIPIENTS
    echo "Attached the AWR Reports for the snapshots $AWR_LOW to $AWR_HIGH for the database $DB_PDB" | mail -s "$SUBJECT" -a $OUTPUT_DIR/$ZIP_FILE $MAIL_RECIPIENTS
    if [ $? -ne 0 ]; then
        echo "Failed to send email to $TO" >&2
    else
        echo "Email sent to $TO successfully"
    fi
}

################################
#
# Main
#
################################
if [ "$MODE" == "SINGLE" ]; then
  echo "Getting AWR report for snapshot $AWR_LOW to snapshot $AWR_HIGH"
  getAwrReport $AWR_LOW $AWR_HIGH
else
  for i in $(seq $AWR_LOW $((AWR_HIGH-1))); do 
    if [[ $ZIP == "true" ]]; then
      files_to_zip+=("$OUTPUT_DIR/AWR_Report_${i}_$((i+1)).html")
    fi
    echo "Getting AWR report for snapshot $i to snapshot $((i+1))"
    getAwrReport $i $((i+1))
  done
fi

if [[ $ZIP == "true" ]]; then
  echo "Creating zip file $ZIP_FILE"
  createZip
  if [[ $DELETE_FILES == "true" ]]; then
    echo "Deleting individual AWR report files"
    rm -fv "${files_to_zip[@]}"
  fi
  if [[ $MAIL_RECIPIENTS != "false" ]]; then
    echo "Sending zip file to $MAIL_RECIPIENTS"
    send_mail
  fi
fi

if [[ $SEND_TO_JASMIN == "true" ]]; then
  echo "Sending AWR reports to jas-min for analysis"
  if [ ! -x "$JASMIN_PATH" ]; then
    echo "jas-min executable not found or not executable at $JASMIN_PATH" >&2
    exit 1
  fi
  $JASMIN_PATH/jas-min -d $OUTPUT_DIR --security-level=1 -W 10 -q --ai google:gemini-2.5-flash:EN $JASMIN_TUNING
fi